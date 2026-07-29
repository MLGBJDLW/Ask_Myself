//! Strictly scoped storage for local media exposed to the desktop WebView.

use crate::error::CoreError;
use image::GenericImageView;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const MAX_AUDIO_BYTES: u64 = 32 * 1024 * 1024;
const MAX_THEME_IMAGE_BYTES: u64 = 24 * 1024 * 1024;
const MAX_THEME_IMAGE_PIXELS: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedAsset {
    pub asset_id: String,
    pub path: PathBuf,
    pub media_type: String,
    pub bytes: u64,
}

#[derive(Debug, Clone)]
pub struct ManagedLocalAssetStore {
    audio_dir: PathBuf,
    theme_dir: PathBuf,
}

impl ManagedLocalAssetStore {
    pub fn new(cache_root: impl Into<PathBuf>, data_root: impl Into<PathBuf>) -> Self {
        Self {
            audio_dir: cache_root.into().join("generated-audio"),
            theme_dir: data_root.into().join("theme-assets"),
        }
    }

    pub fn cache_audio(
        &self,
        source: &Path,
        cache_key_material: &str,
        declared_media_type: &str,
    ) -> Result<ManagedAsset, CoreError> {
        let metadata = fs::metadata(source)?;
        if metadata.len() == 0 || metadata.len() > MAX_AUDIO_BYTES {
            return Err(CoreError::InvalidInput(format!(
                "Generated audio must be between 1 byte and {MAX_AUDIO_BYTES} bytes."
            )));
        }
        let bytes = fs::read(source)?;
        let media_type = detect_audio_media_type(&bytes).ok_or_else(|| {
            CoreError::InvalidInput(
                "Speech provider returned content without a supported audio signature.".into(),
            )
        })?;
        if !media_types_compatible(declared_media_type, media_type) {
            return Err(CoreError::InvalidInput(format!(
                "Speech provider declared {declared_media_type}, but the file signature is {media_type}."
            )));
        }
        fs::create_dir_all(&self.audio_dir)?;
        let asset_id = sha256_hex(cache_key_material.as_bytes());
        let path = self.audio_dir.join(format!(
            "{asset_id}.{}",
            extension_for_media_type(media_type)
        ));
        write_once(&path, &bytes)?;
        Ok(ManagedAsset {
            asset_id,
            path,
            media_type: media_type.into(),
            bytes: bytes.len() as u64,
        })
    }

    pub fn cached_audio(
        &self,
        cache_key_material: &str,
    ) -> Result<Option<ManagedAsset>, CoreError> {
        let asset_id = sha256_hex(cache_key_material.as_bytes());
        for extension in ["wav", "mp3", "ogg", "flac", "m4a", "webm"] {
            let path = self.audio_dir.join(format!("{asset_id}.{extension}"));
            if !path.is_file() {
                continue;
            }
            let bytes = fs::read(&path)?;
            let Some(media_type) = detect_audio_media_type(&bytes) else {
                let _ = fs::remove_file(path);
                continue;
            };
            filetime::set_file_mtime(&path, filetime::FileTime::now())?;
            return Ok(Some(ManagedAsset {
                asset_id,
                path,
                media_type: media_type.into(),
                bytes: bytes.len() as u64,
            }));
        }
        Ok(None)
    }

    pub fn import_theme_background(&self, source: &Path) -> Result<ManagedAsset, CoreError> {
        let metadata = fs::metadata(source)?;
        if metadata.len() == 0 || metadata.len() > MAX_THEME_IMAGE_BYTES {
            return Err(CoreError::InvalidInput(format!(
                "Theme image must be between 1 byte and {MAX_THEME_IMAGE_BYTES} bytes."
            )));
        }
        let bytes = fs::read(source)?;
        let media_type = detect_image_media_type(&bytes).ok_or_else(|| {
            CoreError::InvalidInput(
                "Theme background is not a supported PNG, JPEG, WebP, or GIF image.".into(),
            )
        })?;
        let decoded = image::load_from_memory(&bytes).map_err(|error| {
            CoreError::InvalidInput(format!("Theme background could not be decoded: {error}"))
        })?;
        let (width, height) = decoded.dimensions();
        if u64::from(width).saturating_mul(u64::from(height)) > MAX_THEME_IMAGE_PIXELS {
            return Err(CoreError::InvalidInput(
                "Theme background exceeds the 64-megapixel safety limit.".into(),
            ));
        }
        fs::create_dir_all(&self.theme_dir)?;
        let asset_id = sha256_hex(&bytes);
        let path = self.theme_dir.join(format!(
            "{asset_id}.{}",
            extension_for_media_type(media_type)
        ));
        write_once(&path, &bytes)?;
        Ok(ManagedAsset {
            asset_id,
            path,
            media_type: media_type.into(),
            bytes: bytes.len() as u64,
        })
    }

    pub fn prune_audio_cache(&self, max_bytes: u64, max_age: Duration) -> Result<u64, CoreError> {
        if !self.audio_dir.exists() {
            return Ok(0);
        }
        let now = SystemTime::now();
        let mut entries = fs::read_dir(&self.audio_dir)?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let metadata = entry.metadata().ok()?;
                if !metadata.is_file() {
                    return None;
                }
                Some((
                    entry.path(),
                    metadata.len(),
                    metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                ))
            })
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.2);
        let mut total = entries.iter().map(|entry| entry.1).sum::<u64>();
        let mut removed = 0;
        for (path, size, modified) in entries {
            let expired = now.duration_since(modified).unwrap_or_default() > max_age;
            if !expired && total <= max_bytes {
                continue;
            }
            if fs::remove_file(path).is_ok() {
                total = total.saturating_sub(size);
                removed += 1;
            }
        }
        Ok(removed)
    }

    pub fn clear_audio_cache(&self) -> Result<(u64, u64), CoreError> {
        if !self.audio_dir.exists() {
            return Ok((0, 0));
        }
        let mut removed_files = 0u64;
        let mut removed_bytes = 0u64;
        for entry in fs::read_dir(&self.audio_dir)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if !metadata.is_file() {
                continue;
            }
            fs::remove_file(entry.path())?;
            removed_files += 1;
            removed_bytes = removed_bytes.saturating_add(metadata.len());
        }
        Ok((removed_files, removed_bytes))
    }

    pub fn resolve_theme_background(&self, asset_id: &str) -> Result<ManagedAsset, CoreError> {
        if !is_managed_asset_id(asset_id) {
            return Err(CoreError::InvalidInput(
                "Invalid managed theme asset id.".into(),
            ));
        }
        for extension in ["png", "jpg", "webp", "gif"] {
            let path = self.theme_dir.join(format!("{asset_id}.{extension}"));
            if !path.is_file() {
                continue;
            }
            let bytes = fs::read(&path)?;
            let media_type = detect_image_media_type(&bytes).ok_or_else(|| {
                CoreError::InvalidInput(
                    "Managed theme asset has an invalid image signature.".into(),
                )
            })?;
            return Ok(ManagedAsset {
                asset_id: asset_id.to_string(),
                path,
                media_type: media_type.into(),
                bytes: bytes.len() as u64,
            });
        }
        Err(CoreError::NotFound(format!(
            "Managed theme asset {asset_id}"
        )))
    }

    pub fn garbage_collect_theme_assets(
        &self,
        retained_asset_ids: &[String],
    ) -> Result<(u64, u64), CoreError> {
        if !self.theme_dir.exists() {
            return Ok((0, 0));
        }
        let retained = retained_asset_ids
            .iter()
            .filter(|id| is_managed_asset_id(id))
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let mut removed_files = 0u64;
        let mut removed_bytes = 0u64;
        for entry in fs::read_dir(&self.theme_dir)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if !metadata.is_file() {
                continue;
            }
            let path = entry.path();
            let Some(asset_id) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            if !is_managed_asset_id(asset_id) || retained.contains(asset_id) {
                continue;
            }
            fs::remove_file(path)?;
            removed_files += 1;
            removed_bytes = removed_bytes.saturating_add(metadata.len());
        }
        Ok((removed_files, removed_bytes))
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn is_managed_asset_id(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn detect_audio_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        Some("audio/wav")
    } else if bytes.starts_with(b"ID3")
        || bytes
            .get(0..2)
            .is_some_and(|head| head[0] == 0xff && head[1] & 0xe0 == 0xe0)
    {
        Some("audio/mpeg")
    } else if bytes.starts_with(b"OggS") {
        Some("audio/ogg")
    } else if bytes.starts_with(b"fLaC") {
        Some("audio/flac")
    } else if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        Some("audio/mp4")
    } else if bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
        Some("audio/webm")
    } else {
        None
    }
}

pub fn detect_image_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else {
        None
    }
}

fn media_types_compatible(declared: &str, detected: &str) -> bool {
    let declared = declared
        .split(';')
        .next()
        .unwrap_or(declared)
        .trim()
        .to_ascii_lowercase();
    declared.is_empty()
        || declared == "application/octet-stream"
        || declared == detected
        || (declared == "audio/mp3" && detected == "audio/mpeg")
        || (declared == "audio/opus" && detected == "audio/ogg")
        || (declared == "audio/x-wav" && detected == "audio/wav")
}

fn extension_for_media_type(media_type: &str) -> &'static str {
    match media_type {
        "audio/wav" => "wav",
        "audio/mpeg" => "mp3",
        "audio/ogg" => "ogg",
        "audio/flac" => "flac",
        "audio/mp4" => "m4a",
        "audio/webm" => "webm",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "bin",
    }
}

fn write_once(path: &Path, bytes: &[u8]) -> Result<(), CoreError> {
    if path.exists() {
        return Ok(());
    }
    let temp = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or("asset")
    ));
    fs::write(&temp, bytes)?;
    match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(_error) if path.exists() => {
            let _ = fs::remove_file(temp);
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(temp);
            Err(error.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_signature_detection_rejects_provider_error_pages() {
        assert_eq!(
            detect_audio_media_type(b"RIFFxxxxWAVEdata"),
            Some("audio/wav")
        );
        assert_eq!(detect_audio_media_type(b"ID3\x04\x00"), Some("audio/mpeg"));
        assert_eq!(detect_audio_media_type(br#"{"error":"quota"}"#), None);
        assert_eq!(detect_audio_media_type(b"<html>gateway error</html>"), None);
    }

    #[test]
    fn clearing_audio_cache_preserves_other_cache_directories() {
        let root = std::env::temp_dir().join(format!("nexa-assets-{}", uuid::Uuid::new_v4()));
        let other = root.join("other");
        fs::create_dir_all(root.join("generated-audio")).unwrap();
        fs::create_dir_all(&other).unwrap();
        fs::write(root.join("generated-audio/one.wav"), b"audio").unwrap();
        fs::write(other.join("keep.txt"), b"keep").unwrap();
        let store = ManagedLocalAssetStore::new(&root, root.join("data"));

        assert_eq!(store.clear_audio_cache().unwrap(), (1, 5));
        assert!(other.join("keep.txt").is_file());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn image_signature_detection_is_content_based() {
        assert_eq!(
            detect_image_media_type(b"\x89PNG\r\n\x1a\nrest"),
            Some("image/png")
        );
        assert_eq!(detect_image_media_type(b"wallpaper.png"), None);
    }

    #[test]
    fn theme_gc_only_removes_unreferenced_managed_assets() {
        let root = std::env::temp_dir().join(format!("nexa-theme-assets-{}", uuid::Uuid::new_v4()));
        let data = root.join("data");
        let theme = data.join("theme-assets");
        fs::create_dir_all(&theme).unwrap();
        let keep = "a".repeat(64);
        let remove = "b".repeat(64);
        fs::write(theme.join(format!("{keep}.png")), b"keep").unwrap();
        fs::write(theme.join(format!("{remove}.png")), b"gone").unwrap();
        fs::write(theme.join("unmanaged.txt"), b"safe").unwrap();
        let store = ManagedLocalAssetStore::new(root.join("cache"), &data);

        assert_eq!(
            store
                .garbage_collect_theme_assets(std::slice::from_ref(&keep))
                .unwrap(),
            (1, 4)
        );
        assert!(theme.join(format!("{keep}.png")).is_file());
        assert!(theme.join("unmanaged.txt").is_file());

        let _ = fs::remove_dir_all(root);
    }
}
