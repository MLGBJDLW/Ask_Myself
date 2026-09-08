//! User-imported font files. Only managed, content-addressed copies are exposed
//! to the desktop WebView; source folders and ZIP paths never become asset scopes.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

const MAX_FONT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PACKAGE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_PACKAGE_FONTS: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FontRecord {
    id: String,
    name: String,
    format: String,
    bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FontAsset {
    pub id: String,
    pub name: String,
    pub family: String,
    pub format: String,
    pub path: PathBuf,
    pub bytes: u64,
}

pub struct FontAssetStore {
    root: PathBuf,
}

fn invalid(message: impl Into<String>) -> CoreError {
    CoreError::InvalidInput(message.into())
}

fn font_format(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "ttf" => Some("ttf"),
        "otf" => Some("otf"),
        "woff" => Some("woff"),
        "woff2" => Some("woff2"),
        _ => None,
    }
}

fn validate_container(bytes: &[u8], format: &str) -> Result<(), CoreError> {
    if bytes.len() < 12 || bytes.len() as u64 > MAX_FONT_BYTES {
        return Err(invalid(
            "Each font must be a valid font file no larger than 64 MiB.",
        ));
    }
    let signature = &bytes[..4];
    let expected = match format {
        "ttf" => signature == [0, 1, 0, 0] || signature == b"true",
        "otf" => signature == b"OTTO",
        "woff" => signature == b"wOFF",
        "woff2" => signature == b"wOF2",
        _ => false,
    };
    if !expected {
        return Err(invalid(
            "The font file signature does not match its format.",
        ));
    }
    if matches!(format, "ttf" | "otf") {
        let tables = usize::from(u16::from_be_bytes([bytes[4], bytes[5]]));
        if tables == 0 || tables > 1024 || 12 + tables * 16 > bytes.len() {
            return Err(invalid("The font table directory is incomplete."));
        }
        for table in bytes[12..12 + tables * 16].chunks_exact(16) {
            let offset = u32::from_be_bytes(table[8..12].try_into().unwrap()) as u64;
            let length = u32::from_be_bytes(table[12..16].try_into().unwrap()) as u64;
            if offset + length > bytes.len() as u64 {
                return Err(invalid("A font table extends beyond the file."));
            }
        }
    } else {
        let minimum = if format == "woff2" { 48 } else { 44 };
        let declared = u32::from_be_bytes(bytes[8..12].try_into().unwrap()) as usize;
        if bytes.len() < minimum || declared != bytes.len() {
            return Err(invalid("The compressed font container is incomplete."));
        }
    }
    Ok(())
}

fn valid_id(id: &str) -> bool {
    id.strip_prefix("font-").is_some_and(|hash| {
        hash.len() == 64
            && hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn read_bounded(reader: impl Read, limit: u64) -> Result<Vec<u8>, CoreError> {
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(invalid("The font or font package exceeds its size limit."));
    }
    Ok(bytes)
}

fn write_immutable(path: &Path, bytes: &[u8]) -> Result<(), CoreError> {
    if path.exists() {
        return Ok(());
    }
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = File::options()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        match fs::rename(&temporary, path) {
            Ok(()) => Ok(()),
            Err(_) if path.exists() => Ok(()),
            Err(error) => Err(error),
        }
    })();
    let _ = fs::remove_file(&temporary);
    result.map_err(CoreError::from)
}

impl FontAssetStore {
    pub fn new(data_root: impl AsRef<Path>) -> Self {
        Self {
            root: data_root.as_ref().join("fonts"),
        }
    }

    pub fn import(&self, source: &Path) -> Result<Vec<FontAsset>, CoreError> {
        let metadata = fs::metadata(source)?;
        if !metadata.is_file() || metadata.len() > MAX_PACKAGE_BYTES {
            return Err(invalid(
                "Choose a font file or a ZIP font package no larger than 256 MiB.",
            ));
        }
        let mut pending = Vec::new();
        if source
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
        {
            let mut archive = zip::ZipArchive::new(File::open(source)?)
                .map_err(|error| invalid(format!("Cannot read font package: {error}")))?;
            if archive.len() > 2048 {
                return Err(invalid("The font package contains too many entries."));
            }
            let mut total = 0;
            for index in 0..archive.len() {
                let mut entry = archive
                    .by_index(index)
                    .map_err(|error| invalid(error.to_string()))?;
                if entry.is_dir() {
                    continue;
                }
                let name = entry.name().replace('\\', "/");
                let path = Path::new(&name);
                let Some(format) = font_format(path) else {
                    continue;
                };
                if entry.size() > MAX_FONT_BYTES || pending.len() >= MAX_PACKAGE_FONTS {
                    return Err(invalid(
                        "A font package supports at most 32 fonts, each no larger than 64 MiB.",
                    ));
                }
                let bytes = read_bounded(&mut entry, MAX_FONT_BYTES)?;
                total += bytes.len() as u64;
                if total > MAX_PACKAGE_BYTES {
                    return Err(invalid("Expanded fonts exceed 256 MiB."));
                }
                validate_container(&bytes, format)?;
                pending.push((name, format, bytes));
            }
        } else {
            let format = font_format(source).ok_or_else(|| {
                invalid("Supported fonts: TTF, OTF, WOFF, WOFF2, or ZIP packages.")
            })?;
            let bytes = read_bounded(File::open(source)?, MAX_FONT_BYTES)?;
            validate_container(&bytes, format)?;
            pending.push((source.to_string_lossy().into_owned(), format, bytes));
        }
        if pending.is_empty() {
            return Err(invalid("The package contains no supported font files."));
        }
        fs::create_dir_all(&self.root)?;
        let mut imported = BTreeMap::new();
        for (name, format, bytes) in pending {
            let id = format!("font-{}", blake3::hash(&bytes).to_hex());
            let name: String = Path::new(&name)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .chars()
                .filter(|character| !character.is_control())
                .take(80)
                .collect();
            let record = FontRecord {
                id: id.clone(),
                name,
                format: format.into(),
                bytes: bytes.len() as u64,
            };
            write_immutable(&self.root.join(format!("{id}.{format}")), &bytes)?;
            write_immutable(
                &self.root.join(format!("{id}.json")),
                &serde_json::to_vec(&record)?,
            )?;
            imported.insert(id, self.resolve(&record)?);
        }
        Ok(imported.into_values().collect())
    }

    pub fn list(&self) -> Result<Vec<FontAsset>, CoreError> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let mut assets = Vec::new();
        for entry in fs::read_dir(&self.root)?.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            if !entry.file_type()?.is_file() {
                continue;
            }
            let record = read_bounded(File::open(&path)?, 16 * 1024).and_then(|bytes| {
                serde_json::from_slice::<FontRecord>(&bytes).map_err(CoreError::from)
            });
            if let Ok(record) = record {
                if let Ok(asset) = self.resolve(&record) {
                    assets.push(asset);
                }
            }
        }
        assets.sort_by_cached_key(|asset| asset.name.to_lowercase());
        Ok(assets)
    }

    fn resolve(&self, record: &FontRecord) -> Result<FontAsset, CoreError> {
        if !valid_id(&record.id)
            || !matches!(record.format.as_str(), "ttf" | "otf" | "woff" | "woff2")
        {
            return Err(invalid("Invalid managed font identifier."));
        }
        let path = self.root.join(format!("{}.{}", record.id, record.format));
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.len() != record.bytes || record.bytes > MAX_FONT_BYTES {
            return Err(invalid("The managed font file is missing or has changed."));
        }
        Ok(FontAsset {
            id: record.id.clone(),
            name: record.name.clone(),
            family: format!("NexaFont_{}", &record.id[5..]),
            format: record.format.clone(),
            path,
            bytes: record.bytes,
        })
    }

    pub fn remove(&self, id: &str) -> Result<(), CoreError> {
        if !valid_id(id) {
            return Err(invalid("Invalid managed font identifier."));
        }
        for extension in ["json", "ttf", "otf", "woff", "woff2"] {
            let path = self.root.join(format!("{id}.{extension}"));
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;

    fn container() -> Vec<u8> {
        let mut bytes = vec![0u8; 48];
        bytes[..4].copy_from_slice(b"wOF2");
        bytes[8..12].copy_from_slice(&48u32.to_be_bytes());
        bytes
    }

    #[test]
    fn imported_fonts_are_deduplicated_persisted_and_removed_by_managed_id() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("My Font.woff2");
        fs::write(&source, container()).unwrap();
        let store = FontAssetStore::new(temp.path().join("data"));
        let first = store.import(&source).unwrap();
        let second = store.import(&source).unwrap();
        assert_eq!(first[0].id, second[0].id);
        fs::remove_file(source).unwrap();
        let restored = FontAssetStore::new(temp.path().join("data"))
            .list()
            .unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].name, "My Font");
        assert_eq!(fs::read(&restored[0].path).unwrap(), container());
        store.remove(&first[0].id).unwrap();
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn package_paths_never_become_filesystem_destinations() {
        let temp = tempfile::tempdir().unwrap();
        let package = temp.path().join("fonts.zip");
        let mut archive = zip::ZipWriter::new(File::create(&package).unwrap());
        archive
            .start_file("../../outside.woff2", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(&container()).unwrap();
        archive
            .start_file("ignored/readme.txt", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"license").unwrap();
        archive.finish().unwrap();
        let store = FontAssetStore::new(temp.path().join("data"));
        let imported = store.import(&package).unwrap();
        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].name, "outside");
        assert!(imported[0].path.starts_with(temp.path().join("data/fonts")));
        assert!(!temp.path().join("outside.woff2").exists());
        assert!(store.remove("../../fonts").is_err());
    }

    #[test]
    fn invalid_package_does_not_partially_import_fonts() {
        let temp = tempfile::tempdir().unwrap();
        let package = temp.path().join("fonts.zip");
        let mut archive = zip::ZipWriter::new(File::create(&package).unwrap());
        archive
            .start_file("valid.woff2", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(&container()).unwrap();
        archive
            .start_file("invalid.ttf", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"not a font").unwrap();
        archive.finish().unwrap();
        let store = FontAssetStore::new(temp.path().join("data"));
        assert!(store.import(&package).is_err());
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn truncated_or_replaced_managed_fonts_are_not_exposed() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("font.woff2");
        fs::write(&source, container()).unwrap();
        let store = FontAssetStore::new(temp.path().join("data"));
        let imported = store.import(&source).unwrap();
        fs::write(&imported[0].path, b"broken").unwrap();
        assert!(store.list().unwrap().is_empty());
        assert!(validate_container(b"wOF2\0\0\0\0\0\0\0\x30", "woff2").is_err());
        assert!(validate_container(&container(), "ttf").is_err());
    }
}
