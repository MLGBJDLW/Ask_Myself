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

use crate::app_settings::SpeechToTextConfig;
use crate::error::CoreError;

const WAV_HEADER_BYTES: u64 = 44;
const PCM_CHANNELS: u16 = 1;
const PCM_BITS_PER_SAMPLE: u16 = 16;
const MIN_SAMPLE_RATE: u32 = 8_000;
const MAX_SAMPLE_RATE: u32 = 96_000;
const VOICE_SPOOL_MANIFEST_VERSION: u32 = 2;
/// At process-crash recovery, at most this much acknowledged PCM may trail the
/// last integrity checkpoint. Graceful finalization and shutdown force a full
/// checkpoint before releasing the native file handle.
const VOICE_SPOOL_CHECKPOINT_BYTES: u64 = 1024 * 1024;

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
        if self.max_audio_bytes_per_session > u64::from(u32::MAX - 36) {
            return Err(CoreError::InvalidInput(
                "A PCM WAV spool cannot exceed the RIFF 32-bit data limit".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VoiceSpoolTarget {
    pub provider: String,
    pub api_style: String,
    pub model: String,
    pub configuration_fingerprint_sha256: String,
}

impl VoiceSpoolTarget {
    pub fn from_speech_config(config: &SpeechToTextConfig) -> Result<Self, CoreError> {
        let encoded = serde_json::to_vec(config)?;
        Ok(Self {
            provider: config.provider.trim().to_string(),
            api_style: config.api_style.trim().to_string(),
            model: config.model.trim().to_string(),
            configuration_fingerprint_sha256: format!("{:x}", Sha256::digest(encoded)),
        })
    }
}

impl Default for VoiceSpoolTarget {
    fn default() -> Self {
        Self {
            provider: "local".into(),
            api_style: "local_whisper".into(),
            model: "whisper".into(),
            configuration_fingerprint_sha256: "test-default".into(),
        }
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
    pub target: VoiceSpoolTarget,
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

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum VoiceSpoolLifecycleState {
    Recording,
    Ready,
    DeletionPending,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VoiceSpoolListEntry {
    pub session_id: String,
    pub state: VoiceSpoolLifecycleState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub descriptor: Option<VoiceSpoolDescriptor>,
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
struct VoiceSpoolRecordingManifest {
    version: u32,
    session_id: String,
    created_at_ms: u64,
    sample_rate: u32,
    target: VoiceSpoolTarget,
    checkpoint_audio_bytes: u64,
    checkpoint_next_sequence: u64,
    checkpoint_checksum_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VoiceSpoolDeletionMarker {
    session_id: String,
    requested_at_ms: u64,
}

struct RecordingSession {
    session_id: String,
    created_at_ms: u64,
    sample_rate: u32,
    target: VoiceSpoolTarget,
    next_sequence: u64,
    last_chunk_sha256: Option<[u8; 32]>,
    audio_bytes: u64,
    last_checkpoint_audio_bytes: u64,
    last_checkpoint_next_sequence: u64,
    last_checkpoint_checksum_sha256: String,
    hasher: Sha256,
    file: Option<File>,
    part_path: PathBuf,
    recording_manifest_path: PathBuf,
}

enum VoiceSession {
    Recording(RecordingSession),
    Ready(VoiceSpoolDescriptor),
    DeletionPending {
        descriptor: Option<VoiceSpoolDescriptor>,
        audio_bytes: u64,
    },
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

    pub fn max_chunk_bytes(&self) -> usize {
        self.limits.max_chunk_bytes
    }

    pub fn start(&self, sample_rate: u32) -> Result<VoiceSpoolStarted, CoreError> {
        self.start_for_target(sample_rate, VoiceSpoolTarget::default())
    }

    pub fn start_for_target(
        &self,
        sample_rate: u32,
        target: VoiceSpoolTarget,
    ) -> Result<VoiceSpoolStarted, CoreError> {
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
        let recording_manifest_path = self.recording_manifest_path(&session_id)?;
        let mut open_options = OpenOptions::new();
        open_options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            open_options.mode(0o600);
        }
        let mut file = open_options.open(&part_path)?;
        file.write_all(&[0_u8; WAV_HEADER_BYTES as usize])?;
        file.flush()?;
        file.sync_all()?;
        let created_at_ms = now_ms();
        let empty_checksum_sha256 = format!("{:x}", Sha256::digest([]));
        if let Err(error) = write_recording_manifest(
            &recording_manifest_path,
            &VoiceSpoolRecordingManifest {
                version: VOICE_SPOOL_MANIFEST_VERSION,
                session_id: session_id.clone(),
                created_at_ms,
                sample_rate,
                target: target.clone(),
                checkpoint_audio_bytes: 0,
                checkpoint_next_sequence: 0,
                checkpoint_checksum_sha256: empty_checksum_sha256.clone(),
            },
        ) {
            drop(file);
            let _ = fs::remove_file(&part_path);
            let _ = fs::remove_file(&recording_manifest_path);
            return Err(error);
        }

        sessions.insert(
            session_id.clone(),
            VoiceSession::Recording(RecordingSession {
                session_id: session_id.clone(),
                created_at_ms,
                sample_rate,
                target,
                next_sequence: 0,
                last_chunk_sha256: None,
                audio_bytes: 0,
                last_checkpoint_audio_bytes: 0,
                last_checkpoint_next_sequence: 0,
                last_checkpoint_checksum_sha256: empty_checksum_sha256,
                hasher: Sha256::new(),
                file: Some(file),
                part_path,
                recording_manifest_path,
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
                VoiceSession::DeletionPending { audio_bytes, .. } => *audio_bytes,
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
        let chunk_sha256: [u8; 32] = Sha256::digest(pcm16).into();
        if sequence != recording.next_sequence {
            if recording.next_sequence.checked_sub(1) == Some(sequence)
                && recording.last_chunk_sha256 == Some(chunk_sha256)
            {
                checkpoint_recording(recording, false)?;
                return Ok(VoiceSpoolProgress {
                    session_id: session_id.to_string(),
                    accepted_bytes: pcm16.len(),
                    audio_bytes: recording.audio_bytes,
                    duration_ms: pcm_duration_ms(recording.audio_bytes, recording.sample_rate),
                    next_sequence: recording.next_sequence,
                });
            }
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

        let accepted_len = WAV_HEADER_BYTES.saturating_add(recording.audio_bytes);
        {
            let file = recording
                .file
                .as_mut()
                .expect("recording session owns its spool file");
            if let Err(error) = file.write_all(pcm16) {
                let _ = file.set_len(accepted_len);
                let _ = file.seek(SeekFrom::Start(accepted_len));
                return Err(CoreError::Io(error));
            }
        }
        recording.hasher.update(pcm16);
        recording.audio_bytes = next_session_bytes;
        recording.next_sequence += 1;
        recording.last_chunk_sha256 = Some(chunk_sha256);
        checkpoint_recording(recording, false)?;
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
        let mut recording = match session {
            VoiceSession::Recording(recording) => recording,
            other => {
                sessions.insert(session_id.to_string(), other);
                return Err(CoreError::Conflict(
                    "Voice spool deletion is pending and cannot be finalized".into(),
                ));
            }
        };
        if recording.audio_bytes == 0 {
            drop(recording.file.take());
            let _ = fs::remove_file(&recording.part_path);
            let _ = fs::remove_file(&recording.recording_manifest_path);
            return Err(CoreError::InvalidInput(
                "Voice spool cannot finalize an empty recording".into(),
            ));
        }

        if let Err(error) = checkpoint_recording(&mut recording, true) {
            sessions.insert(session_id.to_string(), VoiceSession::Recording(recording));
            return Err(CoreError::Internal(format!(
                "Voice spool final checkpoint failed; accepted audio remains recoverable for retry: {error}"
            )));
        }

        let recording_manifest_path = recording.recording_manifest_path.clone();
        let recovery_manifest = VoiceSpoolRecordingManifest {
            version: VOICE_SPOOL_MANIFEST_VERSION,
            session_id: session_id.to_string(),
            created_at_ms: recording.created_at_ms,
            sample_rate: recording.sample_rate,
            target: recording.target.clone(),
            checkpoint_audio_bytes: recording.last_checkpoint_audio_bytes,
            checkpoint_next_sequence: recording.last_checkpoint_next_sequence,
            checkpoint_checksum_sha256: recording.last_checkpoint_checksum_sha256.clone(),
        };
        let part_path = recording.part_path.clone();
        let mut file = recording
            .file
            .take()
            .expect("recording session owns its spool file");
        let finalize_result = (|| -> Result<VoiceSpoolDescriptor, CoreError> {
            let header = pcm16_wav_header(recording.sample_rate, recording.audio_bytes)?;
            file.seek(SeekFrom::Start(0))?;
            file.write_all(&header)?;
            file.flush()?;
            file.sync_all()?;
            drop(file);

            let (_, wav_path, manifest_path) = self.paths_for_valid_id(session_id)?;
            fs::rename(&part_path, &wav_path)?;
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
                target: recording.target.clone(),
            };
            if let Err(error) = write_manifest(
                &manifest_path,
                &VoiceSpoolManifest {
                    version: VOICE_SPOOL_MANIFEST_VERSION,
                    descriptor: descriptor.clone(),
                },
            ) {
                let _ = fs::rename(&wav_path, &part_path);
                return Err(error);
            }
            let _ = remove_if_exists(&recording_manifest_path);
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
                match self.recover_recording_session(recovery_manifest, now_ms()) {
                    Ok(Some(descriptor)) => {
                        sessions.insert(
                            session_id.to_string(),
                            VoiceSession::Ready(descriptor.clone()),
                        );
                        Ok(descriptor)
                    }
                    Ok(None) => Err(error),
                    Err(recovery_error) => Err(CoreError::Internal(format!(
                        "Voice spool finalization failed ({error}); accepted audio remains checkpointed but immediate recovery failed ({recovery_error})"
                    ))),
                }
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
                Some(VoiceSession::DeletionPending { .. }) => {
                    return Err(CoreError::Conflict(
                        "Voice spool privacy deletion is pending".into(),
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
                VoiceSession::Recording(_) | VoiceSession::DeletionPending { .. } => None,
            })
            .collect::<Vec<_>>();
        ready.sort_by_key(|descriptor| descriptor.created_at_ms);
        Ok(ready)
    }

    pub fn list(&self) -> Result<Vec<VoiceSpoolListEntry>, CoreError> {
        self.prune_expired()?;
        let sessions = self.lock_sessions()?;
        let mut entries = sessions
            .iter()
            .map(|(session_id, session)| match session {
                VoiceSession::Recording(_) => VoiceSpoolListEntry {
                    session_id: session_id.clone(),
                    state: VoiceSpoolLifecycleState::Recording,
                    descriptor: None,
                },
                VoiceSession::Ready(descriptor) => VoiceSpoolListEntry {
                    session_id: session_id.clone(),
                    state: VoiceSpoolLifecycleState::Ready,
                    descriptor: Some(descriptor.clone()),
                },
                VoiceSession::DeletionPending { descriptor, .. } => VoiceSpoolListEntry {
                    session_id: session_id.clone(),
                    state: VoiceSpoolLifecycleState::DeletionPending,
                    descriptor: descriptor.clone(),
                },
            })
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| {
            entry
                .descriptor
                .as_ref()
                .map(|descriptor| descriptor.created_at_ms)
                .unwrap_or_default()
        });
        Ok(entries)
    }

    pub fn remove(&self, session_id: &str) -> Result<(), CoreError> {
        let mut sessions = self.lock_sessions()?;
        let removed = sessions.remove(session_id);
        let (part_path, wav_path, manifest_path) = self.paths_for_valid_id(session_id)?;
        let recording_manifest_path = self.recording_manifest_path(session_id)?;
        let deletion_path = self.deletion_marker_path(session_id)?;
        let has_managed_files = part_path.exists()
            || wav_path.exists()
            || manifest_path.exists()
            || recording_manifest_path.exists()
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
        let (pending_descriptor, pending_audio_bytes) = match removed.as_ref() {
            Some(VoiceSession::Ready(descriptor)) => {
                (Some(descriptor.clone()), descriptor.audio_bytes)
            }
            Some(VoiceSession::Recording(recording)) => (None, recording.audio_bytes),
            Some(VoiceSession::DeletionPending {
                descriptor,
                audio_bytes,
            }) => (descriptor.clone(), *audio_bytes),
            None => (None, 0),
        };
        drop(removed);
        drop(sessions);
        match self.complete_pending_deletion(session_id) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.lock_sessions()?.insert(
                    session_id.to_string(),
                    VoiceSession::DeletionPending {
                        descriptor: pending_descriptor,
                        audio_bytes: pending_audio_bytes,
                    },
                );
                Err(error)
            }
        }
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
        let mut retained_parts = HashSet::new();
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
            if self.complete_pending_deletion(session_id).is_err() {
                let (part_path, wav_path, manifest_path) = self.paths_for_valid_id(session_id)?;
                let descriptor = fs::read(&manifest_path).ok().and_then(|bytes| {
                    serde_json::from_slice::<VoiceSpoolManifest>(&bytes)
                        .ok()
                        .map(|manifest| manifest.descriptor)
                });
                let audio_bytes = descriptor.as_ref().map_or_else(
                    || {
                        fs::metadata(&wav_path)
                            .or_else(|_| fs::metadata(&part_path))
                            .map(|metadata| metadata.len().saturating_sub(WAV_HEADER_BYTES))
                            .unwrap_or_default()
                    },
                    |descriptor| descriptor.audio_bytes,
                );
                if wav_path.exists() {
                    retained_wavs.insert(wav_path);
                }
                if part_path.exists() {
                    retained_parts.insert(part_path);
                }
                recovered.insert(
                    session_id.to_string(),
                    VoiceSession::DeletionPending {
                        descriptor,
                        audio_bytes,
                    },
                );
            }
        }

        for entry in fs::read_dir(&self.root)? {
            let recording_manifest_path = entry?.path();
            let file_name = recording_manifest_path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            let Some(session_id) = file_name.strip_suffix(".recording.json") else {
                continue;
            };
            if deleting_ids.contains(session_id) {
                continue;
            }
            if Uuid::parse_str(session_id).is_err() {
                remove_if_exists(&recording_manifest_path)?;
                continue;
            }
            let recording_manifest = read_latest_recording_manifest(
                &recording_manifest_path,
                session_id,
                self.limits.max_audio_bytes_per_session,
            )?;
            let Some(recording_manifest) = recording_manifest else {
                let (part_path, wav_path, _) = self.paths_for_valid_id(session_id)?;
                let preserved_path = if part_path.exists() {
                    retained_parts.insert(part_path.clone());
                    Some(part_path)
                } else if wav_path.exists() {
                    retained_wavs.insert(wav_path.clone());
                    Some(wav_path)
                } else {
                    None
                };
                if let Some(path) = preserved_path {
                    let audio_bytes = fs::metadata(path)
                        .map(|metadata| metadata.len().saturating_sub(WAV_HEADER_BYTES))
                        .unwrap_or_default();
                    recovered.insert(
                        session_id.to_string(),
                        VoiceSession::DeletionPending {
                            descriptor: None,
                            audio_bytes,
                        },
                    );
                }
                continue;
            };
            let descriptor = self.recover_recording_session(recording_manifest, current_ms)?;
            if let Some(descriptor) = descriptor {
                let (_, wav_path, _) = self.paths_for_valid_id(&descriptor.session_id)?;
                retained_wavs.insert(wav_path);
                recovered.insert(
                    descriptor.session_id.clone(),
                    VoiceSession::Ready(descriptor),
                );
            }
        }

        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            let file_name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            if entry.file_type()?.is_dir() && file_name.starts_with(".whisper-") {
                fs::remove_dir_all(&path)?;
                continue;
            }
            if (file_name.ends_with(".part") && !retained_parts.contains(&path))
                || file_name.ends_with(".tmp")
            {
                remove_if_exists(&path)?;
            }
        }

        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let manifest_path = entry.path();
            if manifest_path
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| {
                    name.ends_with(".deleting.json") || name.ends_with(".recording.json")
                })
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
                    VoiceSession::Recording(_) | VoiceSession::DeletionPending { .. } => None,
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

    fn recover_recording_session(
        &self,
        manifest: VoiceSpoolRecordingManifest,
        recovered_at_ms: u64,
    ) -> Result<Option<VoiceSpoolDescriptor>, CoreError> {
        let session_id = manifest.session_id;
        let (part_path, wav_path, ready_manifest_path) = self.paths_for_valid_id(&session_id)?;
        let recording_manifest_path = self.recording_manifest_path(&session_id)?;
        if ready_manifest_path.exists() {
            let ready_manifest = fs::read(&ready_manifest_path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<VoiceSpoolManifest>(&bytes).ok());
            if let Some(ready_manifest) = ready_manifest.filter(|ready| {
                ready.version == VOICE_SPOOL_MANIFEST_VERSION
                    && ready.descriptor.session_id == session_id
                    && verify_voice_spool_file(&wav_path, &ready.descriptor).is_ok()
            }) {
                remove_if_exists(&recording_manifest_path)?;
                return Ok(Some(ready_manifest.descriptor));
            }
            return Err(CoreError::Conflict(format!(
                "Voice spool recovery found an invalid ready manifest for {session_id}"
            )));
        }

        let source_path = if part_path.exists() {
            if wav_path.exists() {
                return Err(CoreError::Conflict(format!(
                    "Voice spool recovery found both partial and finalized audio for {session_id}"
                )));
            }
            part_path.clone()
        } else if wav_path.exists() {
            // Finalization may have published the WAV before its ready manifest
            // failed. The durable recording journal still authenticates it.
            wav_path.clone()
        } else {
            remove_if_exists(&recording_manifest_path)?;
            return Ok(None);
        };
        let metadata = match fs::symlink_metadata(&source_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => return Err(CoreError::Io(error)),
        };
        let audio_bytes = manifest.checkpoint_audio_bytes;
        let valid_partial = metadata.file_type().is_file()
            && metadata.len() >= WAV_HEADER_BYTES
            && audio_bytes > 0
            && audio_bytes.is_multiple_of(2)
            && metadata.len() >= WAV_HEADER_BYTES.saturating_add(audio_bytes)
            && audio_bytes <= self.limits.max_audio_bytes_per_session;
        if !valid_partial {
            return Err(CoreError::Conflict(format!(
                "Voice spool recovery has no complete authenticated checkpoint for {session_id}"
            )));
        }

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&source_path)?;
        file.set_len(WAV_HEADER_BYTES.saturating_add(audio_bytes))?;
        file.seek(SeekFrom::Start(WAV_HEADER_BYTES))?;
        let mut hasher = Sha256::new();
        let mut remaining = audio_bytes;
        let mut buffer = [0_u8; 64 * 1024];
        while remaining > 0 {
            let read_len = usize::try_from(remaining.min(buffer.len() as u64))
                .expect("bounded recovery read fits usize");
            file.read_exact(&mut buffer[..read_len])?;
            hasher.update(&buffer[..read_len]);
            remaining -= read_len as u64;
        }
        let checksum_sha256 = format!("{:x}", hasher.finalize());
        if checksum_sha256 != manifest.checkpoint_checksum_sha256 {
            return Err(CoreError::Conflict(format!(
                "Voice spool recovery checkpoint checksum failed for {session_id}"
            )));
        }
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&pcm16_wav_header(manifest.sample_rate, audio_bytes)?)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);

        let moved_from_part = source_path == part_path;
        if moved_from_part {
            fs::rename(&part_path, &wav_path)?;
        }
        let descriptor = VoiceSpoolDescriptor {
            session_id: session_id.clone(),
            audio_bytes,
            duration_ms: pcm_duration_ms(audio_bytes, manifest.sample_rate),
            sample_rate: manifest.sample_rate,
            checksum_sha256,
            created_at_ms: manifest.created_at_ms,
            expires_at_ms: recovered_at_ms.saturating_add(self.limits.retention.as_millis() as u64),
            target: manifest.target,
        };
        if let Err(error) = write_manifest(
            &ready_manifest_path,
            &VoiceSpoolManifest {
                version: VOICE_SPOOL_MANIFEST_VERSION,
                descriptor: descriptor.clone(),
            },
        ) {
            if moved_from_part {
                let _ = fs::rename(&wav_path, &part_path);
            }
            return Err(error);
        }
        remove_if_exists(&recording_manifest_path)?;
        Ok(Some(descriptor))
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

    fn recording_manifest_path(&self, session_id: &str) -> Result<PathBuf, CoreError> {
        Uuid::parse_str(session_id)
            .map_err(|_| CoreError::InvalidInput("Voice spool session id must be a UUID".into()))?;
        Ok(self.root.join(format!("{session_id}.recording.json")))
    }

    fn complete_pending_deletion(&self, session_id: &str) -> Result<(), CoreError> {
        let (part_path, wav_path, manifest_path) = self.paths_for_valid_id(session_id)?;
        let recording_manifest_path = self.recording_manifest_path(session_id)?;
        let deletion_path = self.deletion_marker_path(session_id)?;
        remove_if_exists(&part_path)?;
        remove_if_exists(&wav_path)?;
        remove_if_exists(&manifest_path)?;
        remove_if_exists(&recording_manifest_path)?;
        sync_directory(&self.root)?;
        remove_if_exists(&deletion_path)?;
        sync_directory(&self.root)?;
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
        let Ok(sessions) = self.sessions.get_mut() else {
            return;
        };
        for session in sessions.values_mut() {
            if let VoiceSession::Recording(recording) = session {
                // Normal application shutdown is interruption, not explicit
                // user cancellation. Preserve a complete authenticated
                // checkpoint; `remove` remains the sole privacy-delete path.
                let _ = checkpoint_recording(recording, true);
            }
        }
    }
}

fn checkpoint_recording(recording: &mut RecordingSession, force: bool) -> Result<(), CoreError> {
    let bytes_since_checkpoint = recording
        .audio_bytes
        .saturating_sub(recording.last_checkpoint_audio_bytes);
    let checkpoint_due = recording.audio_bytes > 0
        && (force
            || recording.last_checkpoint_audio_bytes == 0
            || bytes_since_checkpoint >= VOICE_SPOOL_CHECKPOINT_BYTES);
    if !checkpoint_due {
        return Ok(());
    }

    let file = recording
        .file
        .as_mut()
        .expect("recording session owns its spool file");
    file.flush()?;
    file.sync_all()?;
    let checksum_sha256 = format!("{:x}", recording.hasher.clone().finalize());
    append_recording_checkpoint(
        &recording.recording_manifest_path,
        &VoiceSpoolRecordingManifest {
            version: VOICE_SPOOL_MANIFEST_VERSION,
            session_id: recording.session_id.clone(),
            created_at_ms: recording.created_at_ms,
            sample_rate: recording.sample_rate,
            target: recording.target.clone(),
            checkpoint_audio_bytes: recording.audio_bytes,
            checkpoint_next_sequence: recording.next_sequence,
            checkpoint_checksum_sha256: checksum_sha256.clone(),
        },
    )?;
    recording.last_checkpoint_audio_bytes = recording.audio_bytes;
    recording.last_checkpoint_next_sequence = recording.next_sequence;
    recording.last_checkpoint_checksum_sha256 = checksum_sha256;
    Ok(())
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
    let riff_size = 36_u32
        .checked_add(data_size)
        .ok_or_else(|| CoreError::InvalidInput("Voice spool RIFF size overflow".into()))?;
    header[4..8].copy_from_slice(&riff_size.to_le_bytes());
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
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

fn write_recording_manifest(
    path: &Path,
    manifest: &VoiceSpoolRecordingManifest,
) -> Result<(), CoreError> {
    let temp_path = path.with_extension("json.tmp");
    remove_if_exists(&temp_path)?;
    let mut bytes = serde_json::to_vec(manifest)?;
    bytes.push(b'\n');
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
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

fn append_recording_checkpoint(
    path: &Path,
    manifest: &VoiceSpoolRecordingManifest,
) -> Result<(), CoreError> {
    let mut bytes = serde_json::to_vec(manifest)?;
    bytes.push(b'\n');
    let mut file = OpenOptions::new().append(true).open(path)?;
    file.write_all(&bytes)?;
    file.flush()?;
    file.sync_all()?;
    Ok(())
}

fn read_latest_recording_manifest(
    path: &Path,
    expected_session_id: &str,
    max_audio_bytes: u64,
) -> Result<Option<VoiceSpoolRecordingManifest>, CoreError> {
    let bytes = fs::read(path)?;
    let mut latest: Option<VoiceSpoolRecordingManifest> = None;
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let Ok(candidate) = serde_json::from_slice::<VoiceSpoolRecordingManifest>(line) else {
            // A crash may tear only the final append. Earlier complete journal
            // records remain authoritative.
            continue;
        };
        let checksum_valid = candidate.checkpoint_checksum_sha256.len() == 64
            && candidate
                .checkpoint_checksum_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit());
        let structurally_valid = candidate.version == VOICE_SPOOL_MANIFEST_VERSION
            && candidate.session_id == expected_session_id
            && (MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&candidate.sample_rate)
            && candidate.checkpoint_audio_bytes <= max_audio_bytes
            && candidate.checkpoint_audio_bytes.is_multiple_of(2)
            && (candidate.checkpoint_audio_bytes == 0 || candidate.checkpoint_next_sequence > 0)
            && checksum_valid;
        if !structurally_valid {
            continue;
        }
        if let Some(previous) = latest.as_ref() {
            let same_recording = candidate.created_at_ms == previous.created_at_ms
                && candidate.sample_rate == previous.sample_rate
                && candidate.target == previous.target;
            let monotonic = candidate.checkpoint_audio_bytes >= previous.checkpoint_audio_bytes
                && candidate.checkpoint_next_sequence >= previous.checkpoint_next_sequence;
            if !same_recording || !monotonic {
                continue;
            }
        }
        latest = Some(candidate);
    }
    Ok(latest)
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
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), CoreError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), CoreError> {
    // Windows does not expose a portable directory fsync through std. Files
    // and markers are still individually sync_all'd; startup reconciliation
    // completes any persisted deletion marker.
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
        let retry = spool.append(&started.session_id, 0, &[0; 8]).unwrap();
        assert_eq!(retry.audio_bytes, 8);
        assert_eq!(retry.next_sequence, 1);
        assert!(spool.append(&started.session_id, 0, &[1; 8]).is_err());
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
    fn startup_salvages_checkpointed_recording_after_process_loss() {
        let root = tempfile::tempdir().unwrap();
        let session_id = Uuid::new_v4().to_string();
        let part_path = root.path().join(format!("{session_id}.wav.part"));
        let recording_manifest_path = root.path().join(format!("{session_id}.recording.json"));
        let mut bytes = vec![0_u8; WAV_HEADER_BYTES as usize];
        let checkpoint_audio = [1, 0, 2, 0, 3, 0, 4, 0];
        bytes.extend_from_slice(&checkpoint_audio);
        // Simulate a torn even-length write after the last durable journal
        // record. Recovery must never promote these unauthenticated bytes.
        bytes.extend_from_slice(&[9, 9]);
        fs::write(&part_path, bytes).unwrap();
        write_recording_manifest(
            &recording_manifest_path,
            &VoiceSpoolRecordingManifest {
                version: VOICE_SPOOL_MANIFEST_VERSION,
                session_id: session_id.clone(),
                created_at_ms: 1,
                sample_rate: 16_000,
                target: VoiceSpoolTarget::default(),
                checkpoint_audio_bytes: checkpoint_audio.len() as u64,
                checkpoint_next_sequence: 1,
                checkpoint_checksum_sha256: format!("{:x}", Sha256::digest(checkpoint_audio)),
            },
        )
        .unwrap();

        let recovered = VoiceAudioSpool::with_limits(root.path(), test_limits()).unwrap();
        let descriptor = recovered.list_ready().unwrap().pop().unwrap();

        assert_eq!(descriptor.session_id, session_id);
        assert_eq!(descriptor.audio_bytes, 8);
        assert!(!part_path.exists());
        assert!(!recording_manifest_path.exists());
        assert!(recovered.prepare_transcription(&session_id).is_ok());
    }

    #[test]
    fn normal_shutdown_preserves_and_recovers_accepted_audio() {
        let root = tempfile::tempdir().unwrap();
        let (session_id, part_path, recording_manifest_path) = {
            let spool = VoiceAudioSpool::with_limits(root.path(), test_limits()).unwrap();
            let started = spool.start(16_000).unwrap();
            spool.append(&started.session_id, 0, &[0; 8]).unwrap();
            let part_path = spool.paths_for_valid_id(&started.session_id).unwrap().0;
            let manifest_path = spool.recording_manifest_path(&started.session_id).unwrap();
            (started.session_id, part_path, manifest_path)
        };

        assert!(part_path.exists());
        assert!(recording_manifest_path.exists());
        let recovered = VoiceAudioSpool::with_limits(root.path(), test_limits()).unwrap();
        let descriptor = recovered.list_ready().unwrap().pop().unwrap();
        assert_eq!(descriptor.session_id, session_id);
        assert_eq!(descriptor.audio_bytes, 8);
        assert!(recovered.prepare_transcription(&session_id).is_ok());
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
    fn failed_privacy_deletion_stays_visible_and_retryable() {
        let root = tempfile::tempdir().unwrap();
        let spool = VoiceAudioSpool::with_limits(root.path(), test_limits()).unwrap();
        let started = spool.start(16_000).unwrap();
        spool.append(&started.session_id, 0, &[0; 8]).unwrap();
        spool.finish(&started.session_id).unwrap();
        let wav_path = spool.paths_for_valid_id(&started.session_id).unwrap().1;
        fs::remove_file(&wav_path).unwrap();
        fs::create_dir(&wav_path).unwrap();

        assert!(spool.remove(&started.session_id).is_err());
        let entry = spool.list().unwrap().pop().unwrap();
        assert_eq!(entry.session_id, started.session_id);
        assert_eq!(entry.state, VoiceSpoolLifecycleState::DeletionPending);

        fs::remove_dir(&wav_path).unwrap();
        spool.remove(&started.session_id).unwrap();
        assert!(spool.list().unwrap().is_empty());
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
