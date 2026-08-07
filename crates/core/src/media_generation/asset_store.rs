use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::CoreError;

use super::model::{MediaAssetStorageKind, RegisterMediaAssetRequest};

const DEFAULT_MAX_ASSET_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const COPY_BUFFER_BYTES: usize = 128 * 1024;
const SIGNATURE_BYTES: usize = 32;

#[derive(Debug, Clone, PartialEq)]
pub struct ImportMediaAssetRequest {
    pub source_path: PathBuf,
    pub declared_media_type: String,
    pub expected_sha256: Option<String>,
    pub expected_byte_length: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
}

pub(crate) struct ImportedMediaAsset {
    pub registration: RegisterMediaAssetRequest,
    installed_new: bool,
}

/// Write-once application-data store for verified generation media.
#[derive(Debug, Clone)]
pub struct MediaGenerationAssetStore {
    root: PathBuf,
    max_asset_bytes: u64,
}

impl MediaGenerationAssetStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            max_asset_bytes: DEFAULT_MAX_ASSET_BYTES,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_max_asset_bytes(root: impl Into<PathBuf>, max_asset_bytes: u64) -> Self {
        Self {
            root: root.into(),
            max_asset_bytes,
        }
    }

    pub(crate) fn import_verified(
        &self,
        mut request: ImportMediaAssetRequest,
    ) -> Result<ImportedMediaAsset, CoreError> {
        if self.max_asset_bytes == 0 {
            return Err(CoreError::InvalidInput(
                "Media asset byte limit must be greater than zero".to_string(),
            ));
        }
        if !request.source_path.is_file() {
            return Err(CoreError::NotFound(format!(
                "Media asset source {}",
                request.source_path.display()
            )));
        }
        request.declared_media_type = normalize_media_type(&request.declared_media_type)?;
        request.expected_sha256 = request
            .expected_sha256
            .map(|value| normalize_sha256(&value))
            .transpose()?;
        if request.expected_byte_length == Some(0) {
            return Err(CoreError::InvalidInput(
                "Expected media asset byte length must be greater than zero".to_string(),
            ));
        }

        let incoming = self.root.join(".incoming");
        fs::create_dir_all(&incoming)?;
        let temporary_path = incoming.join(format!("{}.part", Uuid::new_v4()));
        let result = self.import_to_temporary(&request, &temporary_path);
        if result.is_err() {
            let _ = fs::remove_file(&temporary_path);
        }
        let (hash, byte_length) = result?;

        let extension = extension_for_media_type(&request.declared_media_type)?;
        let storage_key = format!("sha256/{}/{}.{}", &hash[..2], hash, extension);
        let destination = self.root.join(Path::new(&storage_key));
        let parent = destination.parent().ok_or_else(|| {
            CoreError::Internal("Verified media asset path has no parent".to_string())
        })?;
        fs::create_dir_all(parent)?;
        let installed_new = if destination.exists() {
            verify_existing(&destination, &hash, byte_length)?;
            fs::remove_file(&temporary_path)?;
            false
        } else if let Err(error) = fs::rename(&temporary_path, &destination) {
            if destination.exists() {
                verify_existing(&destination, &hash, byte_length)?;
                let _ = fs::remove_file(&temporary_path);
                false
            } else {
                let _ = fs::remove_file(&temporary_path);
                return Err(CoreError::Io(error));
            }
        } else {
            true
        };

        Ok(ImportedMediaAsset {
            registration: RegisterMediaAssetRequest {
                content_hash_sha256: hash,
                content_verified_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                media_type: request.declared_media_type,
                byte_length,
                storage_kind: MediaAssetStorageKind::ManagedLocal,
                storage_key,
                width: request.width,
                height: request.height,
                duration_ms: request.duration_ms,
            },
            installed_new,
        })
    }

    pub(crate) fn rollback_import(&self, imported: &ImportedMediaAsset) {
        if imported.installed_new {
            let _ = self.delete_storage_key(&imported.registration.storage_key);
        }
    }

    pub(crate) fn delete_storage_key(&self, storage_key: &str) -> Result<(), CoreError> {
        let path = self.checked_storage_path(storage_key)?;
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(CoreError::Io(error)),
        }
    }

    pub(crate) fn reconcile_untracked(
        &self,
        registered_storage_keys: &[String],
    ) -> Result<usize, CoreError> {
        let registered = registered_storage_keys.iter().collect::<HashSet<_>>();
        let mut removed = 0;
        let incoming = self.root.join(".incoming");
        if let Ok(entries) = fs::read_dir(&incoming) {
            for entry in entries {
                let entry = entry?;
                if entry.file_type()?.is_file() {
                    fs::remove_file(entry.path())?;
                    removed += 1;
                }
            }
        }
        let sha_root = self.root.join("sha256");
        let Ok(prefixes) = fs::read_dir(&sha_root) else {
            return Ok(removed);
        };
        for prefix in prefixes {
            let prefix = prefix?;
            if !prefix.file_type()?.is_dir() {
                continue;
            }
            for entry in fs::read_dir(prefix.path())? {
                let entry = entry?;
                if !entry.file_type()?.is_file() {
                    continue;
                }
                let relative = entry
                    .path()
                    .strip_prefix(&self.root)
                    .map_err(|_| CoreError::Internal("Asset path escaped its store".to_string()))?
                    .to_string_lossy()
                    .replace('\\', "/");
                if !registered.contains(&relative) {
                    fs::remove_file(entry.path())?;
                    removed += 1;
                }
            }
        }
        Ok(removed)
    }

    pub fn resolve_storage_key(&self, storage_key: &str) -> Result<PathBuf, CoreError> {
        let path = self.checked_storage_path(storage_key)?;
        if !path.is_file() {
            return Err(CoreError::NotFound(format!(
                "Media generation asset {storage_key}"
            )));
        }
        Ok(path)
    }

    fn checked_storage_path(&self, storage_key: &str) -> Result<PathBuf, CoreError> {
        let relative = Path::new(storage_key);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
            || !storage_key.starts_with("sha256/")
        {
            return Err(CoreError::InvalidInput(
                "Invalid media generation storage key".to_string(),
            ));
        }
        Ok(self.root.join(relative))
    }

    fn import_to_temporary(
        &self,
        request: &ImportMediaAssetRequest,
        temporary_path: &Path,
    ) -> Result<(String, u64), CoreError> {
        let source = File::open(&request.source_path)?;
        let declared_length = source.metadata()?.len();
        if declared_length == 0 || declared_length > self.max_asset_bytes {
            return Err(CoreError::InvalidInput(format!(
                "Media asset must be between 1 and {} bytes",
                self.max_asset_bytes
            )));
        }
        let temporary = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(temporary_path)?;
        let mut reader = BufReader::with_capacity(COPY_BUFFER_BYTES, source);
        let mut writer = BufWriter::with_capacity(COPY_BUFFER_BYTES, temporary);
        let mut hasher = Sha256::new();
        let mut signature = Vec::with_capacity(SIGNATURE_BYTES);
        let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
        let mut byte_length = 0_u64;
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            byte_length = byte_length
                .checked_add(read as u64)
                .ok_or_else(|| CoreError::InvalidInput("Media asset size overflow".to_string()))?;
            if byte_length > self.max_asset_bytes {
                return Err(CoreError::InvalidInput(format!(
                    "Media asset exceeds the {} byte limit",
                    self.max_asset_bytes
                )));
            }
            if signature.len() < SIGNATURE_BYTES {
                let remaining = SIGNATURE_BYTES - signature.len();
                signature.extend_from_slice(&buffer[..read.min(remaining)]);
            }
            hasher.update(&buffer[..read]);
            writer.write_all(&buffer[..read])?;
        }
        if byte_length == 0 {
            return Err(CoreError::InvalidInput(
                "Media asset cannot be empty".to_string(),
            ));
        }
        validate_media_signature(&signature, &request.declared_media_type)?;
        if let Some(expected) = request.expected_byte_length {
            if expected != byte_length {
                return Err(CoreError::Conflict(format!(
                    "Provider declared {expected} bytes but Nexa verified {byte_length}"
                )));
            }
        }
        let hash = format!("{:x}", hasher.finalize());
        if request
            .expected_sha256
            .as_deref()
            .is_some_and(|expected| expected != hash)
        {
            return Err(CoreError::Conflict(
                "Provider-declared SHA-256 does not match the bytes Nexa verified".to_string(),
            ));
        }
        writer.flush()?;
        writer.get_ref().sync_all()?;
        Ok((hash, byte_length))
    }
}

fn verify_existing(
    path: &Path,
    expected_hash: &str,
    expected_length: u64,
) -> Result<(), CoreError> {
    let file = File::open(path)?;
    if file.metadata()?.len() != expected_length {
        return Err(CoreError::Conflict(format!(
            "Existing content-addressed asset {} has a different byte length",
            path.display()
        )));
    }
    let mut reader = BufReader::with_capacity(COPY_BUFFER_BYTES, file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    if format!("{:x}", hasher.finalize()) != expected_hash {
        return Err(CoreError::Conflict(format!(
            "Existing content-addressed asset {} failed SHA-256 verification",
            path.display()
        )));
    }
    Ok(())
}

fn normalize_media_type(value: &str) -> Result<String, CoreError> {
    let value = value
        .split(';')
        .next()
        .unwrap_or(value)
        .trim()
        .to_ascii_lowercase();
    extension_for_media_type(&value)?;
    Ok(value)
}

fn normalize_sha256(value: &str) -> Result<String, CoreError> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.len() != 64 || !normalized.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CoreError::InvalidInput(
            "Expected asset SHA-256 must be 64 hexadecimal characters".to_string(),
        ));
    }
    Ok(normalized)
}

fn validate_media_signature(bytes: &[u8], media_type: &str) -> Result<(), CoreError> {
    let matches = match media_type {
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "image/webp" => bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        "video/mp4" | "audio/mp4" => bytes.len() >= 12 && &bytes[4..8] == b"ftyp",
        "video/webm" | "audio/webm" => bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]),
        "video/quicktime" => {
            bytes.len() >= 12 && &bytes[4..8] == b"ftyp" && &bytes[8..12] == b"qt  "
        }
        "audio/wav" => bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE",
        "audio/mpeg" => {
            bytes.starts_with(b"ID3")
                || bytes
                    .get(0..2)
                    .is_some_and(|head| head[0] == 0xff && head[1] & 0xe0 == 0xe0)
        }
        "audio/ogg" => bytes.starts_with(b"OggS"),
        "audio/flac" => bytes.starts_with(b"fLaC"),
        _ => false,
    };
    if !matches {
        return Err(CoreError::InvalidInput(format!(
            "Media bytes do not match declared type {media_type}"
        )));
    }
    Ok(())
}

fn extension_for_media_type(media_type: &str) -> Result<&'static str, CoreError> {
    match media_type {
        "image/png" => Ok("png"),
        "image/jpeg" => Ok("jpg"),
        "image/gif" => Ok("gif"),
        "image/webp" => Ok("webp"),
        "video/mp4" => Ok("mp4"),
        "video/webm" => Ok("webm"),
        "video/quicktime" => Ok("mov"),
        "audio/wav" => Ok("wav"),
        "audio/mpeg" => Ok("mp3"),
        "audio/ogg" => Ok("ogg"),
        "audio/flac" => Ok("flac"),
        "audio/mp4" => Ok("m4a"),
        "audio/webm" => Ok("webm"),
        _ => Err(CoreError::InvalidInput(format!(
            "Unsupported generated media type {media_type}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_bytes() -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&[0_u8; 64]);
        bytes
    }

    fn request(source_path: PathBuf) -> ImportMediaAssetRequest {
        ImportMediaAssetRequest {
            source_path,
            declared_media_type: "image/png".to_string(),
            expected_sha256: None,
            expected_byte_length: None,
            width: Some(1),
            height: Some(1),
            duration_ms: None,
        }
    }

    #[test]
    fn imports_verified_bytes_once_and_rejects_tampered_expectations() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.png");
        fs::write(&source, png_bytes()).unwrap();
        let store = MediaGenerationAssetStore::new(directory.path().join("assets"));
        let first = store.import_verified(request(source.clone())).unwrap();
        let replay = store.import_verified(request(source.clone())).unwrap();
        assert_eq!(
            first.registration.content_hash_sha256,
            replay.registration.content_hash_sha256
        );
        assert_eq!(
            store
                .resolve_storage_key(&first.registration.storage_key)
                .unwrap(),
            directory
                .path()
                .join("assets")
                .join(&first.registration.storage_key)
        );

        let mut tampered = request(source);
        tampered.expected_sha256 = Some("00".repeat(32));
        assert!(matches!(
            store.import_verified(tampered),
            Err(CoreError::Conflict(_))
        ));
    }

    #[test]
    fn enforces_streaming_bound_and_signature() {
        let directory = tempfile::tempdir().unwrap();
        let oversized = directory.path().join("oversized.png");
        fs::write(&oversized, png_bytes()).unwrap();
        let bounded =
            MediaGenerationAssetStore::with_max_asset_bytes(directory.path().join("bounded"), 16);
        assert!(matches!(
            bounded.import_verified(request(oversized)),
            Err(CoreError::InvalidInput(_))
        ));

        let invalid = directory.path().join("invalid.png");
        fs::write(&invalid, b"not-a-png").unwrap();
        let store = MediaGenerationAssetStore::new(directory.path().join("invalid-assets"));
        assert!(matches!(
            store.import_verified(request(invalid)),
            Err(CoreError::InvalidInput(_))
        ));
    }
}
